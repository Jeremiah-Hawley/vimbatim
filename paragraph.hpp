#include <fstream>
using std::ifstream;

#include "run.hpp"


class paragraph{
  public:
    paragraph(){

    }
    paragraph(ifstream document, int index, string type){

    }
    paragraph(string from_file){

    }
    paragraph(ifstream card_file){

    }


  private:
    int type; //tag = 0, cite = 1, metadata = 2, card = 3, 
    //lines 3-38 don't mean anything so this will contain all the values other than them
  
    string paraID; //paragrah id (dhuh) (psuedorandom 8 digit hexadecimal)
    //string rsidR; //revision save ID, unique id per revision / edit session
    //string rsidRPR; 
    //string rsidRDefault;
    //string rsidP;
    
    run runs[]; //stores the text of the stuff
    string pPr_stype; //stores default style for the paragraph, only truly useful in the case of headings but wtvr
    
    
}


